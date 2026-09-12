use crate::config::ServiceRuntimeConfig;
use axum::body::Body;
use axum::http::header::{HeaderValue, CONTENT_TYPE};
use axum::http::Response;
use std::sync::atomic::{AtomicU64, Ordering};

static METRICS_NAMESPACE: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

// These counters are intentionally process-wide: billing/usage accounting paths can run in
// different crates, but the gateway owns one `/metrics` endpoint. Labels stay fixed so
// an untrusted request cannot create a new time series.
static BILLING_ENRICHMENT_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static BILLING_SETTLEMENT_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static VIDEO_TASK_SETTLEMENT_FAILURES_TOTAL: AtomicU64 = AtomicU64::new(0);
static BILLING_FAIL_OPEN_DAILY_QUOTA_TOTAL: AtomicU64 = AtomicU64::new(0);
static BILLING_FAIL_OPEN_RPM_TOTAL: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricKind {
    Counter,
    Gauge,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricSample {
    pub name: &'static str,
    pub help: &'static str,
    pub kind: MetricKind,
    pub value: u64,
    pub labels: Vec<MetricLabel>,
}

impl MetricSample {
    pub fn new(name: &'static str, help: &'static str, kind: MetricKind, value: u64) -> Self {
        Self {
            name,
            help,
            kind,
            value,
            labels: Vec::new(),
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

    for sample in samples {
        let metric_name = format_metric_name(namespace, sample.name);
        body.push_str(&format!("# HELP {} {}\n", metric_name, sample.help));
        body.push_str(&format!(
            "# TYPE {} {}\n",
            metric_name,
            match sample.kind {
                MetricKind::Counter => "counter",
                MetricKind::Gauge => "gauge",
            }
        ));
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

pub fn record_video_task_settlement_failure() {
    VIDEO_TASK_SETTLEMENT_FAILURES_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_billing_fail_open_daily_quota() {
    BILLING_FAIL_OPEN_DAILY_QUOTA_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_billing_fail_open_rpm() {
    BILLING_FAIL_OPEN_RPM_TOTAL.fetch_add(1, Ordering::Relaxed);
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
        record_billing_settlement_failure, record_video_task_settlement_failure,
        render_prometheus_text, service_up_sample, MetricKind, MetricLabel, MetricSample,
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
        record_video_task_settlement_failure();
        record_billing_fail_open_daily_quota();
        record_billing_fail_open_rpm();
        let samples = billing_metric_samples();
        assert_eq!(samples.len(), 5);
        assert!(samples
            .iter()
            .all(|sample| sample.kind == MetricKind::Counter));
        assert!(samples.iter().all(|sample| sample
            .labels
            .iter()
            .all(|label| matches!(label.key, "component" | "operation"))));
        assert!(samples.iter().all(|sample| sample.value >= 1));
    }
}
