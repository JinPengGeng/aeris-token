//! Latency-distribution (histogram) SLIs owned by the gateway process.
//!
//! These complement the counter/gauge RED metrics from #482 and the stage
//! latency counters: each SLI is a sparse log-bucket histogram per
//! docs/adr/metrics-histogram-evaluation.md. Label sets stay fixed so an
//! untrusted request can never create a new time series.

use std::sync::LazyLock;

use aether_runtime::{LogHistogram, MetricLabel, MetricSample, DEFAULT_LATENCY_BUCKETS_SECONDS};

/// Owns every latency histogram the gateway exports. Production code uses the
/// process-wide [`LATENCY_HISTOGRAMS`] instance; tests construct their own so
/// parallel test threads never observe each other's samples.
pub(crate) struct LatencyHistograms {
    upstream_first_token_seconds: LogHistogram,
    ws_connect_success_seconds: LogHistogram,
    ws_connect_failure_seconds: LogHistogram,
}

impl LatencyHistograms {
    fn new() -> Self {
        Self {
            upstream_first_token_seconds: LogHistogram::new(&DEFAULT_LATENCY_BUCKETS_SECONDS),
            ws_connect_success_seconds: LogHistogram::new(&DEFAULT_LATENCY_BUCKETS_SECONDS),
            ws_connect_failure_seconds: LogHistogram::new(&DEFAULT_LATENCY_BUCKETS_SECONDS),
        }
    }

    /// Time from gateway request start until the first upstream stream byte that
    /// carries provider payload (TTFT proxy) is observed.
    pub(crate) fn record_upstream_first_token_seconds(&self, elapsed_seconds: f64) {
        self.upstream_first_token_seconds
            .observe_seconds(elapsed_seconds);
    }

    /// DNS → TCP → TLS → HTTP 101 Upgrade for an upstream Responses WebSocket.
    /// `success` selects the bounded `outcome=success|failure` label class.
    pub(crate) fn record_ws_connect_seconds(&self, elapsed_seconds: f64, success: bool) {
        if success {
            self.ws_connect_success_seconds
                .observe_seconds(elapsed_seconds);
        } else {
            self.ws_connect_failure_seconds
                .observe_seconds(elapsed_seconds);
        }
    }

    pub(crate) fn metric_samples(&self) -> Vec<MetricSample> {
        vec![
            MetricSample::histogram(
                "upstream_first_token_seconds",
                "Time from gateway request start to the first upstream stream payload byte (TTFT proxy) in seconds.",
                self.upstream_first_token_seconds.snapshot(),
                Vec::new(),
            ),
            MetricSample::histogram(
                "ws_connect_duration_seconds",
                "Upstream Responses WebSocket connect duration (DNS, TCP, TLS, HTTP Upgrade) in seconds.",
                self.ws_connect_success_seconds.snapshot(),
                vec![MetricLabel::new("outcome", "success")],
            ),
            MetricSample::histogram(
                "ws_connect_duration_seconds",
                "Upstream Responses WebSocket connect duration (DNS, TCP, TLS, HTTP Upgrade) in seconds.",
                self.ws_connect_failure_seconds.snapshot(),
                vec![MetricLabel::new("outcome", "failure")],
            ),
        ]
    }
}

static LATENCY_HISTOGRAMS: LazyLock<LatencyHistograms> = LazyLock::new(LatencyHistograms::new);

/// Time from gateway request start until the first upstream stream byte that
/// carries provider payload (TTFT proxy) is observed.
pub(crate) fn record_upstream_first_token_seconds(elapsed_seconds: f64) {
    LATENCY_HISTOGRAMS.record_upstream_first_token_seconds(elapsed_seconds);
}

/// DNS → TCP → TLS → HTTP 101 Upgrade for an upstream Responses WebSocket.
/// `success` selects the bounded `outcome=success|failure` label class.
pub(crate) fn record_ws_connect_seconds(elapsed_seconds: f64, success: bool) {
    LATENCY_HISTOGRAMS.record_ws_connect_seconds(elapsed_seconds, success);
}

pub(crate) fn latency_histogram_metric_samples() -> Vec<MetricSample> {
    LATENCY_HISTOGRAMS.metric_samples()
}

#[cfg(test)]
mod tests {
    use super::LatencyHistograms;

    #[test]
    fn records_bounded_histogram_series() {
        // Own instance: parallel tests recording into the process-wide
        // histograms must not leak samples into this assertion.
        let histograms = LatencyHistograms::new();
        histograms.record_upstream_first_token_seconds(0.12);
        histograms.record_ws_connect_seconds(0.08, true);
        histograms.record_ws_connect_seconds(9.0, false);

        let samples = histograms.metric_samples();
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
