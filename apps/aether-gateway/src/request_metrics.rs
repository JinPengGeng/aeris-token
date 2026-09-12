//! Low-cardinality request RED metrics emitted at the terminal gateway boundary.
//!
//! This module deliberately owns only request metrics. Billing and usage
//! counters live in their respective modules (see #307). Labels are bounded
//! classes so a request id, model, provider id, or error can never become a
//! Prometheus series.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use aether_gateway_frontdoor::telemetry::{normalize_provider_type, normalize_route_class};
use aether_runtime::{MetricKind, MetricLabel, MetricSample};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    route_class: &'static str,
    status_class: &'static str,
    provider: &'static str,
    outcome: &'static str,
}

#[derive(Debug, Default)]
pub(crate) struct RequestMetrics {
    counters: Mutex<BTreeMap<Key, u64>>,
    duration_ms: Mutex<u64>,
    first_byte_ms: Mutex<u64>,
    cancellations: Mutex<u64>,
    retries: Mutex<u64>,
}

impl RequestMetrics {
    pub(crate) fn record(
        &self,
        route_class: Option<&str>,
        status_class: &'static str,
        provider: Option<&str>,
        outcome: &'static str,
        duration_ms: u64,
    ) {
        let key = Key {
            route_class: normalize_route_class(route_class),
            status_class,
            provider: normalize_provider_type(provider),
            outcome,
        };
        *self
            .counters
            .lock()
            .expect("request metrics lock poisoned")
            .entry(key)
            .or_default() += 1;
        let mut total = self
            .duration_ms
            .lock()
            .expect("request duration lock poisoned");
        *total = total.saturating_add(duration_ms);
    }

    pub(crate) fn metric_samples(&self) -> Vec<MetricSample> {
        let counters = self.counters.lock().expect("request metrics lock poisoned");
        let mut samples = counters
            .iter()
            .map(|(key, value)| {
                MetricSample::new(
                    "request_total",
                    "Completed gateway requests by bounded route, status, provider and outcome.",
                    MetricKind::Counter,
                    *value,
                )
                .with_labels(vec![
                    MetricLabel::new("route_class", key.route_class),
                    MetricLabel::new("status_class", key.status_class),
                    MetricLabel::new("provider", key.provider),
                    MetricLabel::new("outcome", key.outcome),
                ])
            })
            .collect::<Vec<_>>();
        samples.extend(
            counters
                .iter()
                .filter(|(key, _)| key.outcome == "error")
                .map(|(key, value)| {
                    MetricSample::new(
                        "request_errors_total",
                        "Gateway requests ending in an upstream or gateway error.",
                        MetricKind::Counter,
                        *value,
                    )
                    .with_labels(vec![
                        MetricLabel::new("route_class", key.route_class),
                        MetricLabel::new("status_class", key.status_class),
                        MetricLabel::new("provider", key.provider),
                    ])
                }),
        );
        let duration_ms = *self
            .duration_ms
            .lock()
            .expect("request duration lock poisoned");
        samples.push(MetricSample::new(
            "request_duration_ms_sum",
            "Sum of terminal gateway request durations in milliseconds.",
            MetricKind::Counter,
            duration_ms,
        ));
        let first_byte_ms = *self
            .first_byte_ms
            .lock()
            .expect("request first-byte metric lock poisoned");
        samples.push(MetricSample::new(
            "request_first_byte_ms_sum",
            "Sum of time to first non-empty response body frame in milliseconds.",
            MetricKind::Counter,
            first_byte_ms,
        ));
        let cancellations = *self
            .cancellations
            .lock()
            .expect("request cancellation metric lock poisoned");
        samples.push(MetricSample::new(
            "request_cancellations_total",
            "Gateway requests cancelled before terminal response completion.",
            MetricKind::Counter,
            cancellations,
        ));
        let retries = *self
            .retries
            .lock()
            .expect("request retry metric lock poisoned");
        samples.push(MetricSample::new(
            "request_retries_total",
            "Internal provider retry attempts; does not increment request_total.",
            MetricKind::Counter,
            retries,
        ));
        samples
    }

    pub(crate) fn record_first_byte(&self, elapsed_ms: u64) {
        let mut total = self
            .first_byte_ms
            .lock()
            .expect("request first-byte metric lock poisoned");
        *total = total.saturating_add(elapsed_ms);
    }

    pub(crate) fn record_cancellation(&self) {
        let mut total = self
            .cancellations
            .lock()
            .expect("request cancellation metric lock poisoned");
        *total = total.saturating_add(1);
    }

    pub(crate) fn record_retry(&self) {
        let mut total = self
            .retries
            .lock()
            .expect("request retry metric lock poisoned");
        *total = total.saturating_add(1);
    }
}

static REQUEST_METRICS: OnceLock<Arc<RequestMetrics>> = OnceLock::new();

pub(crate) fn global_request_metrics() -> &'static Arc<RequestMetrics> {
    REQUEST_METRICS.get_or_init(|| Arc::new(RequestMetrics::default()))
}

#[cfg(test)]
mod tests {
    use super::RequestMetrics;

    #[test]
    fn records_bounded_success_and_error_series() {
        let metrics = RequestMetrics::default();
        metrics.record(Some("ai_public"), "2xx", Some("openai"), "success", 7);
        metrics.record(
            Some("ai_public"),
            "5xx",
            Some("provider-secret"),
            "error",
            9,
        );
        let samples = metrics.metric_samples();
        assert!(samples.iter().any(|sample| sample.name == "request_total"));
        assert!(samples
            .iter()
            .any(|sample| sample.name == "request_errors_total"));
        assert!(samples
            .iter()
            .any(|sample| sample.name == "request_duration_ms_sum" && sample.value == 16));
    }
}
