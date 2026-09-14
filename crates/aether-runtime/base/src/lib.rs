pub mod admission;
mod bootstrap;
pub mod concurrency;
mod config;
pub mod distributed;
mod error;
pub mod metrics;
mod observability;
pub mod queue;
pub mod redaction;
pub mod shutdown;
pub mod task;
mod tracing;

pub use admission::{
    AdmissionPermit, AdmissionPermitHealth, hold_admission_permit_until,
    maybe_hold_axum_response_permit,
};
pub use bootstrap::init_service_runtime;
pub use concurrency::{ConcurrencyError, ConcurrencyGate, ConcurrencyPermit, ConcurrencySnapshot};
pub use config::ServiceRuntimeConfig;
pub use distributed::{
    DistributedConcurrencyError, DistributedConcurrencyGate, DistributedConcurrencyPermit,
    DistributedConcurrencySnapshot,
};
pub use error::RuntimeBootstrapError;
pub use metrics::{
    MetricKind, MetricLabel, MetricSample, prometheus_response, record_billing_enrichment_failure,
    record_billing_fail_open_daily_quota, record_billing_fail_open_rpm,
    record_billing_insufficient_quota, record_billing_settlement_failure,
    record_video_task_settlement_failure, service_up_sample,
};
pub use observability::{
    FileLoggingConfig, LogDestination, LogRotation, ServiceObservabilityConfig,
};
pub use queue::{
    BoundedQueueReceiver, BoundedQueueSender, QueueSendError, QueueSnapshot, bounded_queue,
};
pub use redaction::{TextPayloadSummary, summarize_text_payload};
pub use shutdown::wait_for_shutdown_signal;
pub use tracing::{
    LogFormat, LogReloader, LogShutdownGuard, init_reloadable_service_tracing,
    init_reloadable_tracing, logging_metric_samples, shutdown_logging,
};
