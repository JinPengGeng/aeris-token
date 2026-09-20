use std::sync::atomic::{AtomicU64, Ordering};

use aether_data_contracts::repository::audit::{
    AdminAuditDeliveryRedriveOutcome, AdminAuditDeliverySummary,
};
use aether_runtime::{MetricKind, MetricLabel, MetricSample};

/// Per-Gateway counters shared by request clones. Deliberately no caller data
/// or error text labels; these observations do not provide durable delivery.
#[derive(Debug, Default)]
pub(crate) struct AdminAuditMetrics {
    attempts: AtomicU64,
    failures: AtomicU64,
    timeouts: AtomicU64,
    redrive_redriven: AtomicU64,
    redrive_not_found: AtomicU64,
    redrive_not_dead_letter: AtomicU64,
    redrive_errors: AtomicU64,
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

    pub(crate) fn record_redrive(&self, outcome: AdminAuditDeliveryRedriveOutcome) {
        let counter = match outcome {
            AdminAuditDeliveryRedriveOutcome::Redriven => &self.redrive_redriven,
            AdminAuditDeliveryRedriveOutcome::NotFound => &self.redrive_not_found,
            AdminAuditDeliveryRedriveOutcome::NotDeadLetter => &self.redrive_not_dead_letter,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_redrive_error(&self) {
        self.redrive_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn metric_samples(&self, writer_configured: bool) -> Vec<MetricSample> {
        let mut samples: Vec<_> = vec![
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
        ].into_iter().map(|sample| sample.with_labels(vec![MetricLabel::new("component", "gateway")])).collect();
        for (outcome, value) in [
            ("redriven", self.redrive_redriven.load(Ordering::Relaxed)),
            ("not_found", self.redrive_not_found.load(Ordering::Relaxed)),
            (
                "not_dead_letter",
                self.redrive_not_dead_letter.load(Ordering::Relaxed),
            ),
            ("error", self.redrive_errors.load(Ordering::Relaxed)),
        ] {
            samples.push(
                MetricSample::new(
                    "admin_audit_delivery_redrive_attempts_total",
                    "Administrator audit delivery redrive attempts by bounded outcome.",
                    MetricKind::Counter,
                    value,
                )
                .with_labels(vec![
                    MetricLabel::new("component", "gateway"),
                    MetricLabel::new("outcome", outcome),
                ]),
            );
        }
        samples
    }
}

pub(crate) fn delivery_summary_metric_samples(
    summary: &AdminAuditDeliverySummary,
    now: u64,
) -> Vec<MetricSample> {
    let mut samples = Vec::new();
    for (state, value) in [
        ("pending", summary.pending),
        ("leased", summary.leased),
        ("dead_letter", summary.dead_letter),
    ] {
        samples.push(
            MetricSample::new(
                "admin_audit_delivery_rows",
                "Administrator audit delivery rows by actionable state.",
                MetricKind::Gauge,
                value,
            )
            .with_labels(vec![MetricLabel::new("state", state)]),
        );
    }
    let oldest = summary
        .oldest_unresolved_created_at
        .map(|v| v.timestamp().max(0) as u64);
    samples.push(MetricSample::new(
        "admin_audit_delivery_oldest_unresolved_age_seconds",
        "Age of the oldest unresolved administrator audit delivery.",
        MetricKind::Gauge,
        oldest.map(|v| now.saturating_sub(v)).unwrap_or_default(),
    ));
    samples
}

#[cfg(test)]
mod delivery_tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn delivery_summary_metrics_are_stable_and_low_cardinality() {
        let empty = delivery_summary_metric_samples(&AdminAuditDeliverySummary::default(), 100);
        assert_eq!(empty.len(), 4);
        assert_eq!(empty.last().unwrap().value, 0);
        let populated = delivery_summary_metric_samples(
            &AdminAuditDeliverySummary {
                pending: 2,
                leased: 3,
                dead_letter: 4,
                oldest_unresolved_created_at: Some(Utc.timestamp_opt(40, 0).unwrap()),
            },
            100,
        );
        let states = populated
            .iter()
            .flat_map(|sample| sample.labels.iter())
            .filter(|label| label.key == "state")
            .map(|label| label.value.as_str())
            .collect::<Vec<_>>();
        assert_eq!(states, ["pending", "leased", "dead_letter"]);
        assert_eq!(populated.last().unwrap().value, 60);
        assert!(populated
            .iter()
            .flat_map(|sample| sample.labels.iter())
            .all(|label| matches!(label.key, "state" | "outcome" | "component")));
    }
}
