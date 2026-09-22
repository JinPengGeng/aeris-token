//! Latency-distribution (histogram) SLIs owned by the gateway process.
//!
//! These complement the counter/gauge RED metrics from #482 and the stage
//! latency counters: each SLI is a sparse log-bucket histogram per
//! docs/adr/metrics-histogram-evaluation.md. Label sets stay fixed so an
//! untrusted request can never create a new time series.

use std::sync::LazyLock;

use aether_runtime::{LogHistogram, MetricLabel, MetricSample, DEFAULT_LATENCY_BUCKETS_SECONDS};

static UPSTREAM_FIRST_TOKEN_SECONDS: LazyLock<LogHistogram> =
    LazyLock::new(|| LogHistogram::new(&DEFAULT_LATENCY_BUCKETS_SECONDS));
static WS_CONNECT_SUCCESS_SECONDS: LazyLock<LogHistogram> =
    LazyLock::new(|| LogHistogram::new(&DEFAULT_LATENCY_BUCKETS_SECONDS));
static WS_CONNECT_FAILURE_SECONDS: LazyLock<LogHistogram> =
    LazyLock::new(|| LogHistogram::new(&DEFAULT_LATENCY_BUCKETS_SECONDS));

/// Time from gateway request start until the first upstream stream byte that
/// carries provider payload (TTFT proxy) is observed.
pub(crate) fn record_upstream_first_token_seconds(elapsed_seconds: f64) {
    UPSTREAM_FIRST_TOKEN_SECONDS.observe_seconds(elapsed_seconds);
}

/// DNS → TCP → TLS → HTTP 101 Upgrade for an upstream Responses WebSocket.
/// `success` selects the bounded `outcome=success|failure` label class.
pub(crate) fn record_ws_connect_seconds(elapsed_seconds: f64, success: bool) {
    if success {
        WS_CONNECT_SUCCESS_SECONDS.observe_seconds(elapsed_seconds);
    } else {
        WS_CONNECT_FAILURE_SECONDS.observe_seconds(elapsed_seconds);
    }
}

pub(crate) fn latency_histogram_metric_samples() -> Vec<MetricSample> {
    vec![
        MetricSample::histogram(
            "upstream_first_token_seconds",
            "Time from gateway request start to the first upstream stream payload byte (TTFT proxy) in seconds.",
            UPSTREAM_FIRST_TOKEN_SECONDS.snapshot(),
            Vec::new(),
        ),
        MetricSample::histogram(
            "ws_connect_duration_seconds",
            "Upstream Responses WebSocket connect duration (DNS, TCP, TLS, HTTP Upgrade) in seconds.",
            WS_CONNECT_SUCCESS_SECONDS.snapshot(),
            vec![MetricLabel::new("outcome", "success")],
        ),
        MetricSample::histogram(
            "ws_connect_duration_seconds",
            "Upstream Responses WebSocket connect duration (DNS, TCP, TLS, HTTP Upgrade) in seconds.",
            WS_CONNECT_FAILURE_SECONDS.snapshot(),
            vec![MetricLabel::new("outcome", "failure")],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::{
        latency_histogram_metric_samples, record_upstream_first_token_seconds,
        record_ws_connect_seconds,
    };

    #[test]
    fn records_bounded_histogram_series() {
        record_upstream_first_token_seconds(0.12);
        record_ws_connect_seconds(0.08, true);
        record_ws_connect_seconds(9.0, false);

        let samples = latency_histogram_metric_samples();
        assert_eq!(samples.len(), 3);
        let ttft = samples[0]
            .histogram
            .as_ref()
            .expect("first token histogram");
        assert_eq!(ttft.count, 1);
        assert_eq!(ttft.sum, 0.12);
        let success = samples[1]
            .histogram
            .as_ref()
            .expect("success ws connect histogram");
        assert_eq!(success.count, 1);
        assert_eq!(success.sum, 0.08);
        let failure = samples[2]
            .histogram
            .as_ref()
            .expect("failure ws connect histogram");
        assert_eq!(failure.count, 1);
        assert_eq!(failure.sum, 9.0);
        assert!(samples[0].labels.is_empty());
        assert!(samples.iter().skip(1).all(|sample| sample.labels
            == vec![aether_runtime::MetricLabel::new("outcome", "success")]
            || sample.labels == vec![aether_runtime::MetricLabel::new("outcome", "failure")]));
    }
}
