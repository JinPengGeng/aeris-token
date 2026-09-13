use std::sync::atomic::{AtomicU64, Ordering};

use aether_runtime::{MetricKind, MetricLabel, MetricSample};

/// Per-Gateway counters shared by request clones. Deliberately no caller data
/// or error text labels; these observations do not provide durable delivery.
#[derive(Debug, Default)]
pub(crate) struct AdminAuditMetrics {
    attempts: AtomicU64,
    failures: AtomicU64,
    timeouts: AtomicU64,
}

impl AdminAuditMetrics {
    pub(super) fn record_attempt(&self) {
        self.attempts.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn record_failure(&self, timed_out: bool) {
        self.failures.fetch_add(1, Ordering::Relaxed);
        if timed_out {
            self.timeouts.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn metric_samples(&self, writer_configured: bool) -> Vec<MetricSample> {
        vec![
            MetricSample::new(
                "admin_audit_persist_attempts_total",
                "Administrator audit persistence invocations, including idempotent replays.",
                MetricKind::Counter,
                self.attempts.load(Ordering::Relaxed),
            ),
            MetricSample::new(
                "admin_audit_persist_failures_total",
                "Administrator audit persistence failures, including missing writer, errors and timeouts.",
                MetricKind::Counter,
                self.failures.load(Ordering::Relaxed),
            ),
            MetricSample::new(
                "admin_audit_persist_timeouts_total",
                "Administrator audit persistence timeouts; SQL may still commit after this observation.",
                MetricKind::Counter,
                self.timeouts.load(Ordering::Relaxed),
            ),
            MetricSample::new(
                "durable_admin_audit_available",
                "Whether an audit writer is configured; not database health or delivery success.",
                MetricKind::Gauge,
                u64::from(writer_configured),
            ),
        ]
        .into_iter()
        .map(|sample| sample.with_labels(vec![MetricLabel::new("component", "gateway")]))
        .collect()
    }
}
