//! Shared runtime foundation for Aether components: admission control,
//! concurrency limits, metrics, queues, graceful shutdown, task helpers, and
//! bootstrap/config plumbing reused across gateway binaries.
#![warn(missing_docs)]

/// Module: admission.
pub mod admission;
mod bootstrap;
/// Module: concurrency.
pub mod concurrency;
mod config;
mod error;
/// Module: metrics.
pub mod metrics;
mod observability;
/// Module: queue.
pub mod queue;
/// Module: redaction.
pub mod redaction;
/// Module: shutdown.
pub mod shutdown;
/// Module: task.
pub mod task;
mod tracing;

pub use admission::{
    hold_admission_permit_until, maybe_hold_axum_response_permit, AdmissionPermit,
    AdmissionPermitHealth,
};
pub use bootstrap::init_service_runtime;
pub use concurrency::{ConcurrencyError, ConcurrencyGate, ConcurrencyPermit, ConcurrencySnapshot};
pub use config::ServiceRuntimeConfig;
pub use error::RuntimeBootstrapError;
pub use metrics::{
    prometheus_response, record_billing_enrichment_failure, record_billing_fail_open_daily_quota,
    record_billing_fail_open_rpm, record_billing_insufficient_quota,
    record_billing_settlement_duration_seconds, record_billing_settlement_failure,
    record_video_task_settlement_failure, service_up_sample, LogHistogram, MetricHistogram,
    MetricKind, MetricLabel, MetricSample, DEFAULT_LATENCY_BUCKETS_SECONDS,
};
pub use observability::{
    FileLoggingConfig, LogDestination, LogRotation, ServiceObservabilityConfig,
};
pub use queue::{
    bounded_queue, BoundedQueueReceiver, BoundedQueueSender, QueueSendError, QueueSnapshot,
};
pub use redaction::{summarize_text_payload, TextPayloadSummary};
pub use shutdown::wait_for_shutdown_signal;
pub use tracing::{
    init_reloadable_service_tracing, init_reloadable_tracing, logging_metric_samples,
    shutdown_logging, LogFormat, LogReloader, LogShutdownGuard,
};
