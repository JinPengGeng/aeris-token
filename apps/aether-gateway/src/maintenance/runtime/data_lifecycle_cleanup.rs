use aether_data::{DataLayerError, StatsRetentionCleanupSummary};

use crate::data::GatewayDataState;

use super::{now_unix_secs, system_config_bool, system_config_u64, system_config_usize};

const SECS_PER_DAY: u64 = 24 * 60 * 60;
const STATS_HOURLY_RETENTION_DAYS_DEFAULT: u64 = 180;
const STATS_DAILY_RETENTION_DAYS_DEFAULT: u64 = 730;
const VIDEO_TASK_TERMINAL_RETENTION_DAYS_DEFAULT: u64 = 30;
const STATS_HOURLY_RETENTION_DAYS_MIN: u64 = 7;
const STATS_HOURLY_RETENTION_DAYS_MAX: u64 = 1_825;
const STATS_DAILY_RETENTION_DAYS_MIN: u64 = 30;
const STATS_DAILY_RETENTION_DAYS_MAX: u64 = 3_650;
const VIDEO_TASK_TERMINAL_RETENTION_DAYS_MIN: u64 = 1;
const VIDEO_TASK_TERMINAL_RETENTION_DAYS_MAX: u64 = 365;
const DATA_LIFECYCLE_CLEANUP_BATCH_SIZE_DEFAULT: usize = 5_000;
const DATA_LIFECYCLE_CLEANUP_BATCH_SIZE_MAX: usize = 50_000;
/// Cap per-run batches so a first run after a long gap drains gradually
/// instead of monopolizing the pool.
const DATA_LIFECYCLE_MAX_BATCHES_PER_RUN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DataLifecycleCleanupSettings {
    pub stats_hourly_retention_days: u64,
    pub stats_daily_retention_days: u64,
    pub video_task_terminal_retention_days: u64,
    pub batch_size: usize,
}

pub(super) async fn data_lifecycle_cleanup_settings(
    data: &GatewayDataState,
) -> Result<DataLifecycleCleanupSettings, DataLayerError> {
    let stats_hourly_retention_days = system_config_u64(
        data,
        "stats_hourly_retention_days",
        STATS_HOURLY_RETENTION_DAYS_DEFAULT,
    )
    .await?
    .clamp(
        STATS_HOURLY_RETENTION_DAYS_MIN,
        STATS_HOURLY_RETENTION_DAYS_MAX,
    );
    let stats_daily_retention_days = system_config_u64(
        data,
        "stats_daily_retention_days",
        STATS_DAILY_RETENTION_DAYS_DEFAULT,
    )
    .await?
    .clamp(
        STATS_DAILY_RETENTION_DAYS_MIN.max(stats_hourly_retention_days),
        STATS_DAILY_RETENTION_DAYS_MAX,
    );
    let video_task_terminal_retention_days = system_config_u64(
        data,
        "video_task_terminal_retention_days",
        VIDEO_TASK_TERMINAL_RETENTION_DAYS_DEFAULT,
    )
    .await?
    .clamp(
        VIDEO_TASK_TERMINAL_RETENTION_DAYS_MIN,
        VIDEO_TASK_TERMINAL_RETENTION_DAYS_MAX,
    );
    let fallback_batch_size = system_config_usize(data, "cleanup_batch_size", 0).await?;
    let batch_size = (if fallback_batch_size > 0 {
        fallback_batch_size
    } else {
        DATA_LIFECYCLE_CLEANUP_BATCH_SIZE_DEFAULT
    })
    .clamp(1, DATA_LIFECYCLE_CLEANUP_BATCH_SIZE_MAX);

    Ok(DataLifecycleCleanupSettings {
        stats_hourly_retention_days,
        stats_daily_retention_days,
        video_task_terminal_retention_days,
        batch_size,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct DataLifecycleCleanupSummary {
    pub stats: StatsRetentionCleanupSummary,
    pub video_tasks_deleted: u64,
}

pub(super) async fn cleanup_data_lifecycle_once(
    data: &GatewayDataState,
) -> Result<DataLifecycleCleanupSummary, DataLayerError> {
    cleanup_data_lifecycle_at(data, now_unix_secs()).await
}

pub(super) async fn cleanup_data_lifecycle_at(
    data: &GatewayDataState,
    now_unix_secs: u64,
) -> Result<DataLifecycleCleanupSummary, DataLayerError> {
    if !system_config_bool(data, "enable_auto_cleanup", true).await? {
        return Ok(DataLifecycleCleanupSummary::default());
    }
    let settings = data_lifecycle_cleanup_settings(data).await?;
    let hourly_before = now_unix_secs.saturating_sub(
        settings
            .stats_hourly_retention_days
            .saturating_mul(SECS_PER_DAY),
    );
    let daily_before = now_unix_secs.saturating_sub(
        settings
            .stats_daily_retention_days
            .saturating_mul(SECS_PER_DAY),
    );
    let video_before = now_unix_secs.saturating_sub(
        settings
            .video_task_terminal_retention_days
            .saturating_mul(SECS_PER_DAY),
    );

    let mut summary = DataLifecycleCleanupSummary::default();
    for _ in 0..DATA_LIFECYCLE_MAX_BATCHES_PER_RUN {
        let batch = data
            .cleanup_stats_aggregates(hourly_before, daily_before, settings.batch_size)
            .await?;
        let batch_total = batch
            .hourly_rows_deleted
            .saturating_add(batch.daily_rows_deleted);
        summary.stats.hourly_rows_deleted = summary
            .stats
            .hourly_rows_deleted
            .saturating_add(batch.hourly_rows_deleted);
        summary.stats.daily_rows_deleted = summary
            .stats
            .daily_rows_deleted
            .saturating_add(batch.daily_rows_deleted);
        if usize::try_from(batch_total).unwrap_or(usize::MAX) < settings.batch_size {
            break;
        }
    }
    for _ in 0..DATA_LIFECYCLE_MAX_BATCHES_PER_RUN {
        let deleted = data
            .cleanup_terminal_video_tasks(video_before, settings.batch_size)
            .await?;
        summary.video_tasks_deleted = summary.video_tasks_deleted.saturating_add(deleted);
        if usize::try_from(deleted).unwrap_or(usize::MAX) < settings.batch_size {
            break;
        }
    }
    Ok(summary)
}
