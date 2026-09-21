use crate::config::ServiceRuntimeConfig;
use axum::body::Body;
use axum::http::header::{HeaderValue, CONTENT_TYPE};
use axum::http::Response;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};

static METRICS_NAMESPACE: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

// These counters are intentionally process-wide: billing/usage accounting paths can run in
// different crates, but the gateway owns one `/metrics` endpoint. Labels stay fixed so
// an untrusted request cannot create a new time series.
static BILLING_ENRICHMENT_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static BILLING_SETTLEMENT_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static BILLING_INSUFFICIENT_QUOTA_TOTAL: AtomicU64 = AtomicU64::new(0);
static VIDEO_TASK_SETTLEMENT_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static BILLING_FAIL_OPEN_DAILY_QUOTA_TOTAL: AtomicU64 = AtomicU64::new(0);
static BILLING_FAIL_OPEN_RPM_TOTAL: AtomicU64 = AtomicU64::new(0);
static BILLING_SETTLEMENT_DURATION: std::sync::LazyLock<LogHistogram> =
    std::sync::LazyLock::new(|| LogHistogram::new(&DEFAULT_LATENCY_BUCKETS_SECONDS));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    Counter,
    Gauge,
    Histogram,
}

/// Sparse log-bucket histogram boundaries, in seconds, following
/// docs/adr/metrics-histogram-evaluation.md: 10 finite buckets cover the
/// 5ms–5s gateway/upstream latency domain; `+Inf` is implicit in rendering.
/// Second-based bounds keep the families promtool-lint clean.
pub const DEFAULT_LATENCY_BUCKETS_SECONDS: [f64; 10] =
    [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0];

/// Immutable snapshot of a histogram observation set: exclusive per-bucket
/// counts plus the running sum of observed values. Rendered cumulatively by
/// `render_prometheus_text` as `_bucket{le=}` / `_sum` / `_count`.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricHistogram {
    pub buckets: &'static [f64],
    pub counts: Vec<u64>,
    pub sum: f64,
    pub count: u64,
}

/// Process-wide sparse log-bucket histogram recorder. One atomic store per
/// observation on the hot path; cumulative buckets are reconstructed at
/// scrape time by the renderer.
#[derive(Debug)]
pub struct LogHistogram {
    buckets: &'static [f64],
    counts: Vec<AtomicU64>,
    sum: AtomicU64,
}

impl LogHistogram {
    pub fn new(buckets: &'static [f64]) -> Self {
        assert!(
            buckets.windows(2).all(|pair| pair[0] < pair[1]),
            "histogram buckets must be strictly increasing"
        );
        Self {
            buckets,
            counts: buckets.iter().map(|_| AtomicU64::new(0)).collect(),
            sum: AtomicU64::new(0),
        }
    }

    pub fn observe_seconds(&self, value_seconds: f64) {
        let value_seconds = if value_seconds.is_nan() || value_seconds < 0.0 {
            0.0
        } else {
            value_seconds
        };
        let index = self
            .buckets
            .iter()
            .position(|bucket| value_seconds <= *bucket)
            .unwrap_or(self.buckets.len() - 1);
        self.counts[index].fetch_add(1, Ordering::Relaxed);
        let finite = if value_seconds.is_finite() {
            value_seconds
        } else {
            0.0
        };
        let _ = self
            .sum
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some((f64::from_bits(current) + finite).to_bits())
            });
    }

    pub fn snapshot(&self) -> MetricHistogram {
        MetricHistogram {
            buckets: self.buckets,
            counts: self
                .counts
                .iter()
                .map(|count| count.load(Ordering::Relaxed))
                .collect(),
            sum: f64::from_bits(self.sum.load(Ordering::Relaxed)),
            count: self
                .counts
                .iter()
                .map(|count| count.load(Ordering::Relaxed))
                .sum(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricLabel {
    pub key: &'static str,
    pub value: String,
}

impl MetricLabel {
    pub fn new(key: &'static str, value: impl Into<String>) -> Self {
        Self {
            key,
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MetricSample {
    pub name: &'static str,
    pub help: &'static str,
    pub kind: MetricKind,
    pub value: u64,
    pub labels: Vec<MetricLabel>,
    pub histogram: Option<MetricHistogram>,
}

impl MetricSample {
    pub fn new(name: &'static str, help: &'static str, kind: MetricKind, value: u64) -> Self {
        Self {
            name,
            help,
            kind,
            value,
            labels: Vec::new(),
            histogram: None,
        }
    }

    pub fn histogram(
        name: &'static str,
        help: &'static str,
        histogram: MetricHistogram,
        labels: Vec<MetricLabel>,
    ) -> Self {
        Self {
            name,
            help,
            kind: MetricKind::Histogram,
            value: 0,
            labels,
            histogram: Some(histogram),
        }
    }

    pub fn with_labels(mut self, labels: Vec<MetricLabel>) -> Self {
        self.labels = labels;
        self
    }
}

pub fn init_metrics(config: ServiceRuntimeConfig) {
    let _ = METRICS_NAMESPACE.set(config.observability.metrics_namespace);
}

pub fn metrics_namespace() -> Option<&'static str> {
    METRICS_NAMESPACE.get().copied()
}

pub fn render_prometheus_text(samples: &[MetricSample]) -> String {
    let mut body = String::new();
    let namespace = metrics_namespace();
    let mut declared_families = BTreeSet::new();

    for sample in samples {
        let metric_name = format_metric_name(namespace, sample.name);
        // Prometheus permits one HELP/TYPE declaration per family, even when
        // that family contains multiple (possibly nonadjacent) label sets.
        if declared_families.insert(sample.name) {
            body.push_str(&format!("# HELP {} {}\n", metric_name, sample.help));
            body.push_str(&format!(
                "# TYPE {} {}\n",
                metric_name,
                match sample.kind {
                    MetricKind::Counter => "counter",
                    MetricKind::Gauge => "gauge",
                    MetricKind::Histogram => "histogram",
                }
            ));
        }
        if let Some(histogram) = sample.histogram.as_ref() {
            render_histogram_sample(&mut body, &metric_name, sample, histogram);
            continue;
        }
        body.push_str(&metric_name);
        if !sample.labels.is_empty() {
            body.push('{');
            for (index, label) in sample.labels.iter().enumerate() {
                if index > 0 {
                    body.push(',');
                }
                body.push_str(label.key);
                body.push_str("=\"");
                body.push_str(&escape_prometheus_label(&label.value));
                body.push('"');
            }
            body.push('}');
        }
        body.push(' ');
        body.push_str(&sample.value.to_string());
        body.push('\n');
    }

    body
}

fn render_histogram_sample(
    body: &mut String,
    metric_name: &str,
    sample: &MetricSample,
    histogram: &MetricHistogram,
) {
    debug_assert_eq!(histogram.counts.len(), histogram.buckets.len());
    let mut cumulative = 0_u64;
    for (bucket, count) in histogram.buckets.iter().zip(histogram.counts.iter()) {
        cumulative = cumulative.saturating_add(*count);
        body.push_str(&format!(
            "{}_bucket{} {}\n",
            metric_name,
            render_bucket_labels(&sample.labels, &format_bucket_bound(*bucket)),
            cumulative,
        ));
    }
    body.push_str(&format!(
        "{}_bucket{} {}\n",
        metric_name,
        render_bucket_labels(&sample.labels, "+Inf"),
        histogram.count,
    ));
    body.push_str(&format!(
        "{}_sum{} {}\n",
        metric_name,
        render_labels(&sample.labels),
        format_histogram_float(histogram.sum),
    ));
    body.push_str(&format!(
        "{}_count{} {}\n",
        metric_name,
        render_labels(&sample.labels),
        histogram.count,
    ));
}

/// `{key="value",...}` including the braces, or an empty string when the
/// sample carries no labels.
fn render_labels(labels: &[MetricLabel]) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let mut rendered = String::with_capacity(64);
    rendered.push('{');
    for (index, label) in labels.iter().enumerate() {
        if index > 0 {
            rendered.push(',');
        }
        rendered.push_str(label.key);
        rendered.push_str("=\"");
        rendered.push_str(&escape_prometheus_label(&label.value));
        rendered.push('"');
    }
    rendered.push('}');
    rendered
}

/// Bucket label sets append `le` after the fixed sample labels.
fn render_bucket_labels(labels: &[MetricLabel], le: &str) -> String {
    if labels.is_empty() {
        return format!("{{le=\"{le}\"}}");
    }
    let mut rendered = render_labels(labels);
    rendered.insert(rendered.len() - 1, ',');
    rendered.insert_str(rendered.len() - 1, &format!("le=\"{le}\""));
    rendered
}

fn format_bucket_bound(bound_seconds: f64) -> String {
    format_histogram_float(bound_seconds)
}

fn format_histogram_float(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

pub fn prometheus_response(samples: &[MetricSample]) -> Response<Body> {
    let mut response = Response::new(Body::from(render_prometheus_text(samples)));
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    response
}

pub fn service_up_sample(service: &'static str) -> MetricSample {
    MetricSample::new(
        "service_up",
        "Whether the service process is currently up.",
        MetricKind::Gauge,
        1,
    )
    .with_labels(vec![MetricLabel::new("service", service)])
}

pub fn record_billing_enrichment_failure() {
    BILLING_ENRICHMENT_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_billing_settlement_failure() {
    BILLING_SETTLEMENT_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_billing_insufficient_quota() {
    BILLING_INSUFFICIENT_QUOTA_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_video_task_settlement_failure() {
    VIDEO_TASK_SETTLEMENT_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_billing_fail_open_daily_quota() {
    BILLING_FAIL_OPEN_DAILY_QUOTA_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_billing_fail_open_rpm() {
    BILLING_FAIL_OPEN_RPM_TOTAL.fetch_add(1, Ordering::Relaxed);
}

/// Record how long one terminal usage settlement took, from settlement entry
/// to completion, regardless of outcome. Called from the shared
/// usage-settlement wrapper so every settlement path is covered once.
pub fn record_billing_settlement_duration_seconds(elapsed_seconds: f64) {
    BILLING_SETTLEMENT_DURATION.observe_seconds(elapsed_seconds);
}

pub(crate) fn billing_metric_samples() -> Vec<MetricSample> {
    vec![
        MetricSample::new(
            "billing_enrichment_failures_total",
            "Terminal usage events whose billing enrichment failed before persistence.",
            MetricKind::Counter,
            BILLING_ENRICHMENT_FAILURES_TOTAL.load(Ordering::Relaxed),
        )
        .with_labels(vec![
            MetricLabel::new("component", "usage"),
            MetricLabel::new("operation", "enrichment"),
        ]),
        MetricSample::new(
            "billing_settlement_failures_total",
            "Terminal usage events whose settlement operation failed.",
            MetricKind::Counter,
            BILLING_SETTLEMENT_FAILURES_TOTAL.load(Ordering::Relaxed),
        )
        .with_labels(vec![
            MetricLabel::new("component", "usage"),
            MetricLabel::new("operation", "settlement"),
        ]),
        MetricSample::new(
            "billing_insufficient_quota_total",
            "Completed usage settlements recorded without a wallet debit because credit was insufficient.",
            MetricKind::Counter,
            BILLING_INSUFFICIENT_QUOTA_TOTAL.load(Ordering::Relaxed),
        )
        .with_labels(vec![
            MetricLabel::new("component", "billing"),
            MetricLabel::new("operation", "insufficient_quota"),
        ]),
        MetricSample::new(
            "billing_video_task_settlement_failures_total",
            "Video task terminal usage events whose settlement operation failed.",
            MetricKind::Counter,
            VIDEO_TASK_SETTLEMENT_FAILURES_TOTAL.load(Ordering::Relaxed),
        )
        .with_labels(vec![
            MetricLabel::new("component", "video_task"),
            MetricLabel::new("operation", "settlement"),
        ]),
        MetricSample::new(
            "billing_fail_open_total",
            "Billing protection checks that deliberately allowed traffic while their runtime backend was unavailable.",
            MetricKind::Counter,
            BILLING_FAIL_OPEN_DAILY_QUOTA_TOTAL.load(Ordering::Relaxed),
        )
        .with_labels(vec![
            MetricLabel::new("component", "gateway"),
            MetricLabel::new("operation", "daily_quota"),
        ]),
        MetricSample::new(
            "billing_fail_open_total",
            "Billing protection checks that deliberately allowed traffic while their runtime backend was unavailable.",
            MetricKind::Counter,
            BILLING_FAIL_OPEN_RPM_TOTAL.load(Ordering::Relaxed),
        )
        .with_labels(vec![
            MetricLabel::new("component", "gateway"),
            MetricLabel::new("operation", "rpm"),
        ]),
        MetricSample::histogram(
            "billing_settlement_duration_seconds",
            "Terminal usage settlement durations in seconds.",
            BILLING_SETTLEMENT_DURATION.snapshot(),
            vec![
                MetricLabel::new("component", "usage"),
                MetricLabel::new("operation", "settlement"),
            ],
        ),
    ]
}

fn format_metric_name(namespace: Option<&str>, name: &str) -> String {
    match namespace {
        Some(namespace) if !namespace.is_empty() => format!("{}_{}", namespace, name),
        _ => name.to_string(),
    }
}

fn escape_prometheus_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::{
        billing_metric_samples, prometheus_response, record_billing_enrichment_failure,
        record_billing_fail_open_daily_quota, record_billing_fail_open_rpm,
        record_billing_insufficient_quota, record_billing_settlement_duration_seconds,
        record_billing_settlement_failure, record_video_task_settlement_failure,
        render_prometheus_text, service_up_sample, LogHistogram, MetricKind, MetricLabel,
        MetricSample, DEFAULT_LATENCY_BUCKETS_SECONDS,
    };
    use axum::body::to_bytes;

    #[test]
    fn renders_prometheus_samples_with_labels() {
        let text = render_prometheus_text(&[MetricSample::new(
            "queue_depth",
            "Current queue depth",
            MetricKind::Gauge,
            3,
        )
        .with_labels(vec![MetricLabel::new("queue", "proxy_writer")])]);

        assert!(text.contains("# HELP queue_depth Current queue depth"));
        assert!(text.contains("# TYPE queue_depth gauge"));
        assert!(text.contains("queue_depth{queue=\"proxy_writer\"} 3"));
    }

    #[test]
    fn escapes_prometheus_labels() {
        let text = render_prometheus_text(&[MetricSample::new(
            "errors_total",
            "Errors",
            MetricKind::Counter,
            1,
        )
        .with_labels(vec![MetricLabel::new("message", "bad\"line\nx")])]);

        assert!(text.contains("message=\"bad\\\"line\\nx\""));
    }

    #[test]
    fn declares_each_metric_family_once_without_losing_label_variants() {
        let counter = |operation| {
            MetricSample::new("fail_open_total", "Guard failures", MetricKind::Counter, 2)
                .with_labels(vec![MetricLabel::new("operation", operation)])
        };
        let text = render_prometheus_text(&[
            counter("daily_quota"),
            service_up_sample("gateway"),
            counter("rpm"),
        ]);
        assert_eq!(text.matches("# HELP fail_open_total ").count(), 1);
        assert_eq!(text.matches("# TYPE fail_open_total counter").count(), 1);
        assert!(text.contains("fail_open_total{operation=\"daily_quota\"} 2"));
        assert!(text.contains("fail_open_total{operation=\"rpm\"} 2"));
        assert!(text.contains("service_up{service=\"gateway\"} 1"));
    }

    #[test]
    fn renders_histogram_with_cumulative_buckets_sum_and_count() {
        let histogram = LogHistogram::new(&DEFAULT_LATENCY_BUCKETS_SECONDS);
        for value in [0.003, 0.007, 0.007, 0.042, 0.042, 0.042, 9.0] {
            histogram.observe_seconds(value);
        }
        let sample = MetricSample::histogram(
            "gateway_request_duration_seconds",
            "Gateway request duration.",
            histogram.snapshot(),
            vec![MetricLabel::new("route_class", "ai_public")],
        );

        let text = render_prometheus_text(&[sample]);

        assert!(text.contains("# TYPE gateway_request_duration_seconds histogram"));
        // le bounds are rendered in increasing order and stay cumulative.
        let le_positions = ["le=\"0.005\"", "le=\"0.01\"", "le=\"0.025\"", "le=\"+Inf\""]
            .map(|needle| text.find(needle).expect("le bound should render"));
        assert!(le_positions.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(text.contains(
            "gateway_request_duration_seconds_bucket{route_class=\"ai_public\",le=\"0.005\"} 1"
        ));
        assert!(text.contains(
            "gateway_request_duration_seconds_bucket{route_class=\"ai_public\",le=\"0.01\"} 3"
        ));
        assert!(text.contains(
            "gateway_request_duration_seconds_bucket{route_class=\"ai_public\",le=\"+Inf\"} 7"
        ));
        // The +Inf bucket always equals _count, and _sum matches observations.
        assert!(
            text.contains("gateway_request_duration_seconds_count{route_class=\"ai_public\"} 7")
        );
        assert!(
            text.contains("gateway_request_duration_seconds_sum{route_class=\"ai_public\"} 9.143"),
            "unexpected render:\n{text}"
        );
    }

    #[test]
    fn renders_histogram_without_labels() {
        let histogram = LogHistogram::new(&DEFAULT_LATENCY_BUCKETS_SECONDS);
        histogram.observe_seconds(0.001);
        let text = render_prometheus_text(&[MetricSample::histogram(
            "queue_dwell_seconds",
            "Queue dwell.",
            histogram.snapshot(),
            Vec::new(),
        )]);

        assert!(text.contains("queue_dwell_seconds_bucket{le=\"0.005\"} 1"));
        assert!(text.contains("queue_dwell_seconds_bucket{le=\"+Inf\"} 1"));
        assert!(text.contains("queue_dwell_seconds_count 1"));
        assert!(text.contains("queue_dwell_seconds_sum 0.001"));
    }

    #[test]
    fn log_histogram_observes_into_expected_exclusive_buckets() {
        let histogram = LogHistogram::new(&DEFAULT_LATENCY_BUCKETS_SECONDS);
        histogram.observe_seconds(0.0);
        histogram.observe_seconds(0.005);
        histogram.observe_seconds(0.00501);
        histogram.observe_seconds(f64::NAN);
        histogram.observe_seconds(f64::INFINITY);
        let snapshot = histogram.snapshot();
        assert_eq!(snapshot.counts[0], 3); // 0.0, 0.005 and NaN clamp into the lowest bucket
        assert_eq!(snapshot.counts[1], 1);
        assert_eq!(snapshot.counts[9], 1); // +Inf lands in the top finite bucket
        assert_eq!(snapshot.count, 5);
        assert!(snapshot.sum.is_finite());
    }

    #[test]
    fn histogram_output_parses_as_prometheus_text_lines() {
        let histogram = LogHistogram::new(&DEFAULT_LATENCY_BUCKETS_SECONDS);
        for value in [0.012, 0.3] {
            histogram.observe_seconds(value);
        }
        let text = render_prometheus_text(&[MetricSample::histogram(
            "billing_settlement_duration_seconds",
            "Settlement durations.",
            histogram.snapshot(),
            vec![
                MetricLabel::new("component", "usage"),
                MetricLabel::new("operation", "settlement"),
            ],
        )]);

        let mut saw_inf = false;
        for line in text.lines() {
            if line.starts_with('#') {
                continue;
            }
            let (head, raw_value) = line.rsplit_once(' ').expect("line should have a value");
            let value: f64 = raw_value.parse().expect("value should be numeric");
            assert!(value.is_finite());
            if head.contains("le=\"+Inf\"") {
                saw_inf = true;
                assert!(head.starts_with("billing_settlement_duration_seconds_bucket{"));
            }
        }
        assert!(saw_inf);
    }

    #[tokio::test]
    async fn builds_prometheus_http_response() {
        let response = prometheus_response(&[service_up_sample("gateway")]);
        let content_type = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok());
        assert_eq!(
            content_type,
            Some("text/plain; version=0.0.4; charset=utf-8")
        );
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let text = String::from_utf8(body.to_vec()).expect("body should be utf8");
        assert!(text.contains("service_up{service=\"gateway\"} 1"));
    }

    #[test]
    fn billing_metrics_use_fixed_low_cardinality_labels() {
        record_billing_enrichment_failure();
        record_billing_settlement_failure();
        record_billing_insufficient_quota();
        record_video_task_settlement_failure();
        record_billing_fail_open_daily_quota();
        record_billing_fail_open_rpm();
        record_billing_settlement_duration_seconds(0.0125);
        let samples = billing_metric_samples();
        assert_eq!(samples.len(), 7);
        assert!(samples.iter().all(|sample| {
            sample.kind == MetricKind::Counter || sample.kind == MetricKind::Histogram
        }));
        assert!(samples.iter().all(|sample| {
            sample
                .labels
                .iter()
                .all(|label| matches!(label.key, "component" | "operation"))
        }));
        assert!(samples
            .iter()
            .filter(|sample| sample.kind == MetricKind::Counter)
            .all(|sample| sample.value >= 1));
        let histogram = samples
            .iter()
            .find(|sample| sample.name == "billing_settlement_duration_seconds")
            .and_then(|sample| sample.histogram.as_ref())
            .expect("settlement duration histogram should be exported");
        assert_eq!(histogram.count, 1);
        assert_eq!(histogram.sum, 0.0125);
    }
}
