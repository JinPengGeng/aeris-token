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
    hold_admission_permit_until, maybe_hold_axum_response_permit, AdmissionPermit,
    AdmissionPermitHealth,
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
    prometheus_response, record_billing_enrichment_failure, record_billing_fail_open_daily_quota,
    record_billing_fail_open_rpm, record_billing_settlement_failure,
    record_video_task_settlement_failure, service_up_sample, MetricKind, MetricLabel, MetricSample,
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
